// A conversation drawn in the page (`world-gui.md`, 对话). Each block keeps
// its element for as long as what it shows does not change, so a stream of
// words redraws one paragraph, not the conversation.
//
// What a turn did between the things it said is a stretch of rows under one
// heading. The heading stands as long as there is a row under it, keeps its
// element while rows come and go, and says what is happening, or once done
// what the stretch did. Everything that folds starts folded and opens only
// when the reader opens it; it opens and closes in motion, the line that was
// pressed staying where it is (`../longview/anchor.ts`). Long text under a
// row shows its first lines, then grows into a box that draws only what is
// in view.
//
// What the World told the session is drawn from the facts it carries, as a
// quiet line on the right; what was done to a turn (copy, ask again, fork)
// appears only once the turn is over.

import type { Content, MachineName, Notice, Speaker, Tool } from "../world/events";
import { STOPPED_BY_USER } from "../world/events";
import type { Attachment } from "../world/attachments";
import type { JsonValue } from "../world/wire";
import { answerOf, drawAsk, readAsk, type AskWords } from "./asks";
import type { Words } from "../words";
import { glyph } from "./glyphs";
import { visible, type Activity, type Conversation, type Entry, type SendState, type ShownAttachment } from "./model";
import { escape, plain, render } from "./prose";
import { counts, numbered, parse, renderRows } from "./diff";
import { headline, groupHeading, tenseOf } from "./tool";
import { drawDiagrams } from "./diagrams";
import { readTold, worldSaid, type WorldSaid } from "./told";
import { detailOf } from "../app/problem";
import { AnchorLayout, AnchoredHeight } from "../longview";
import { LongText } from "./long-text";
import { replyOf } from "./async-ask";
import { chosenIn, drawQuestions, type QuestionWords } from "./question-card";

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
}

type ToolEntry = Extract<Entry, { kind: "tool" }>;
type AskEntry = Extract<Entry, { kind: "ask" }>;
type HeardEntry = Extract<Entry, { kind: "heard" }>;
type QuestionEntry = Extract<Entry, { kind: "question" }>;

/** One stretch on the page: its heading, its box and the rows in it. */
interface Stretch {
  element: HTMLElement;
  head: HTMLButtonElement;
  box: HTMLElement;
  rows: HTMLElement;
  drawn: Map<string, { signature: string; element: HTMLElement }>;
}

export class ConversationView {
  private openRows = new Set<string>();
  private openStretches = new Set<string>();
  private drawn = new Map<string, { signature: string; element: HTMLElement }>();
  private stretches = new Map<string, Stretch>();
  /** Long texts open under rows, by row, to be let go when the row is. */
  private longs = new Map<string, LongText[]>();
  private motions = new WeakMap<HTMLElement, AnchoredHeight>();
  /** Rows already on the page once; only rows new to a live conversation
   *  make an entrance. */
  private seen = new Set<string>();
  private settled = false;
  /** Questions being answered again, and ones the user skipped. */
  private editing = new Set<string>();
  private skipped = new Set<string>();
  private readonly layout: AnchorLayout;
  private ticking = 0;
  /** The line at the foot of a turn under way: what it is doing and for how
   *  long. Above it is what the session has taken in; below it, words sent
   *  that it has not read yet. */
  private readonly status: HTMLElement;

  constructor(
    private readonly root: HTMLElement,
    private readonly conversation: Conversation,
    private readonly context: Context,
  ) {
    root.classList.add("transcript");
    this.layout = new AnchorLayout(context.scroller, root);
    this.status = document.createElement("div");
    this.status.className = "turn-status";
    this.status.setAttribute("role", "status");
    this.status.innerHTML = `<span class="turn-status-mark" aria-hidden="true"></span><span class="turn-status-step"></span><span class="turn-status-time"></span>`;
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
      const acts = block?.querySelector<HTMLElement>(":scope > .turn-acts:not(.latest)");
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

  /** Brings the page up to the conversation as it now stands. */
  update(): void {
    // Words sent and not yet read stand below the turn's line, in order.
    const all = this.conversation.blocks();
    const pending = all.filter((block) => block.kind === "heard" && block.sending !== undefined);
    const blocks = [...all.filter((block) => !pending.includes(block)), ...pending];
    const present = new Set<string>();
    let previous: HTMLElement | null = null;
    const statusShown = this.drawStatus();
    let statusPlaced = false;
    const place = (element: HTMLElement) => {
      const want: Element | null = previous ? previous.nextElementSibling : this.root.firstElementChild;
      if (want !== element) {
        if (previous) previous.after(element);
        else this.root.prepend(element);
      }
      previous = element;
    };
    for (const block of blocks) {
      if (statusShown && !statusPlaced && pending.includes(block)) {
        place(this.status);
        statusPlaced = true;
      }
      present.add(block.key);
      let element: HTMLElement;
      if (block.kind === "activity-group") element = this.stretch(block).element;
      else {
        const signature = this.signature(block);
        let drawn = this.drawn.get(block.key);
        if (!drawn || drawn.signature !== signature) {
          const fresh = this.draw(block);
          if (drawn) {
            this.letGo(block.key);
            drawn.element.replaceWith(fresh);
          }
          if (this.openRows.has(block.key)) this.fill(block.key, fresh);
          drawn = { signature, element: fresh };
          this.drawn.set(block.key, drawn);
        }
        element = drawn.element;
      }
      place(element);
    }
    if (statusShown && !statusPlaced) place(this.status);
    if (!statusShown) this.status.remove();
    for (const [key, drawn] of this.drawn) {
      if (!present.has(key)) {
        drawn.element.remove();
        this.drawn.delete(key);
      }
    }
    for (const [key, stretch] of this.stretches) {
      if (!present.has(key)) {
        for (const row of stretch.drawn.keys()) this.letGo(row);
        stretch.element.remove();
        this.stretches.delete(key);
      }
    }
    this.settled = true;
    this.tick();
    void drawDiagrams(this.root);
  }

  private signature(block: Entry): string {
    switch (block.kind) {
      case "prose":
        return `${block.key}p${block.version}${block.writing ? "w" : ""}${this.lastOfTurn(block) ? `l${this.conversation.turnEnded(block.turn) ? "e" : ""}${this.latest(block.turn) ? "n" : ""}` : ""}`;
      case "heard":
        return `${block.key}h${JSON.stringify(block.sending ?? null)}${block.attached?.length ?? 0}${block.turn ?? ""}${block.turn && this.conversation.turnEnded(block.turn) ? "e" : ""}${block.turn && this.latest(block.turn) ? "n" : ""}`;
      case "steps":
        return `${block.key}s${JSON.stringify(block.steps)}${block.note ?? ""}`;
      case "ask":
        return `${block.key}a${block.version}`;
      case "question":
        return `${block.key}q${block.version}${this.editing.has(block.key) ? "e" : ""}${this.skipped.has(block.key) ? "s" : ""}`;
      default:
        return block.key;
    }
  }

  private memberSignature(entry: Entry): string {
    switch (entry.kind) {
      case "prose":
        return `${entry.key}p${entry.version}${entry.writing ? "w" : ""}`;
      case "tool":
        return `${entry.key}t${entry.tool.status}${entry.ended?.use ?? "-"}${entry.tool.output.length}${this.conversation.agentOf(entry) ? `a${this.conversation.agentStamp}` : ""}`;
      default:
        return entry.key;
    }
  }

  /** The turn whose actions stay shown: the last that ended. */
  private latest(turn: string): boolean {
    const ended = this.conversation.entries.filter((entry) => entry.kind === "ended");
    return ended[ended.length - 1]?.kind === "ended" && (ended[ended.length - 1] as Extract<Entry, { kind: "ended" }>).turn === turn;
  }

  private draw(block: Entry): HTMLElement {
    const element = document.createElement("div");
    element.dataset.key = block.key;
    element.className = `block block-${block.kind}`;
    if (block.kind === "prose") element.classList.add(`prose-${block.prose}`);
    if (block.kind === "heard") {
      const notice = this.noticeOf(block);
      element.classList.add(notice ? "from-world" : !block.from || block.from.kind === "user" ? "from-user" : "from-other");
      if (block.reply) element.classList.add("from-reply");
    }
    element.innerHTML = this.entry(block);
    return element;
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
        return `<div class="note caution">${escape(entry.health.text)}</div>`;
      case "uncertain":
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
  private lastOfTurn(entry: Extract<Entry, { kind: "prose" }>): boolean {
    const later = this.conversation.entries.slice(this.conversation.entries.indexOf(entry) + 1);
    return entry.prose === "message" && !later.some((other) => other.kind === "prose" && other.prose === "message" && other.turn === entry.turn);
  }

  /** Under a turn's last message, once the turn is over: copy its answer,
   *  ask again, fork from here, and what the turn took. The latest turn's
   *  stay shown; earlier ones come with the pointer. */
  private answerActs(entry: Extract<Entry, { kind: "prose" }>): string {
    if (entry.writing || !this.lastOfTurn(entry) || !this.conversation.turnEnded(entry.turn)) return "";
    const { words } = this.context;
    const ended = this.conversation.entries.find((other): other is Extract<Entry, { kind: "ended" }> => other.kind === "ended" && other.turn === entry.turn);
    const took = ended?.end.duration_ms ? words.took(ended.end.duration_ms) : "";
    const icon = (act: string, name: Parameters<typeof glyph>[0], label: string, extra = "") =>
      `<button class="turn-icon" type="button" ${act} aria-label="${escape(label)}" title="${escape(label)}"${extra}>${glyph(name, 16)}</button>`;
    return `<div class="turn-acts answer-acts${this.latest(entry.turn) ? " latest" : ""}">${icon(`data-copy-turn="${escape(entry.turn)}"`, "copy", words.copyAnswer)}${icon(
      `data-turn-act="regenerate" data-turn="${escape(entry.turn)}"`,
      "regenerate",
      words.regenerate,
      ' data-needs="rewind"',
    )}${icon(`data-turn-act="fork" data-turn="${escape(entry.turn)}"`, "fork", words.fork, ' data-needs="fork"')}${took ? `<span class="turn-icon turn-info" tabindex="0" title="${escape(took)}" aria-label="${escape(took)}">${glyph("info", 16)}</span>` : ""}</div>`;
  }

  /** What the user attached to their words, above them: pictures, one
   *  larger or several in a row, and the other files as chips. */
  private attached(items: ShownAttachment[]): string {
    if (!items.length) return "";
    const pictures = items.filter((item) => item.image && (item.preview || item.kept));
    const files = items.filter((item) => !pictures.includes(item));
    const picture = (item: ShownAttachment) =>
      `<li class="bubble-picture"><img data-picture ${item.preview ? `src="${escape(item.preview)}"` : `data-attachment="${escape(JSON.stringify(item.kept))}"`} alt="${escape(item.name)}" title="${escape(item.name)}" tabindex="0"></li>`;
    const file = (item: ShownAttachment) => {
      const label = `${glyph(item.image ? "image" : "file", 16)}<span class="bubble-file-name">${escape(item.name)}</span>${item.size === null ? "" : `<span class="bubble-file-size">${escape(sizeOf(item.size))}</span>`}`;
      return item.kept
        ? `<li><a class="bubble-file" href="#" data-file="${escape(item.kept.path)}" data-file-machine="${escape(item.kept.owner.machine)}">${label}</a></li>`
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
    if (ask.kind === "files" && ask.item && !ask.paths.length) {
      const tool = this.conversation.entries.find(
        (candidate): candidate is ToolEntry => candidate.kind === "tool" && candidate.item === ask.item && candidate.turn === entry.turn,
      );
      if (tool?.tool.does.as === "edit") ask.paths = tool.tool.does.changes.map((change) => change.path);
    }
    return drawAsk(entry.key, ask, entry.state, this.context.residentMachine, asks.words);
  }

  /** Questions asked without waiting: a card to answer, answered, or
   *  skipped and a way back to it. */
  private question(entry: QuestionEntry): string {
    const questions = this.context.questions;
    if (!questions) return `<div class="prose">${render(entry.questions.map((q) => q.title).join("\n\n"))}</div>`;
    if (this.skipped.has(entry.key) && !this.editing.has(entry.key)) {
      return `<div class="question-card skipped"><p class="question-title">${plain(entry.questions[0]!.title)}</p><div class="question-foot"><button class="text-button quiet-button" type="button" data-question-change="${entry.key}">${escape(questions.words.answer)}</button></div></div>`;
    }
    return drawQuestions(entry.key, entry.questions, entry.answers, questions.words, this.editing.has(entry.key));
  }

  private async answerQuestions(target: HTMLElement): Promise<void> {
    const questions = this.context.questions;
    const card = target.closest<HTMLElement>(".question-card");
    const key = target.dataset.questionSend ?? target.dataset.questionSkip ?? target.dataset.questionChange ?? "";
    const entry = this.conversation.entries.find((candidate): candidate is QuestionEntry => candidate.kind === "question" && candidate.key === key);
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
    const info = acts.querySelector<HTMLElement>(".turn-info");
    if (info) sheet.insertAdjacentHTML("beforeend", `<p class="act-sheet-info">${escape(info.title)}</p>`);
    scrim.addEventListener("click", close);
    document.body.append(scrim, sheet);
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
    const entry = this.conversation.entries.find((candidate): candidate is AskEntry => candidate.kind === "ask" && candidate.key === target.dataset.ask);
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
    return `<div class="turn-acts user-acts${entry.turn && this.latest(entry.turn) ? " latest" : ""}"><button class="turn-icon" type="button" data-copy="${entry.key}" aria-label="${escape(words.copyMessage)}" title="${escape(words.copyMessage)}">${glyph("copy", 16)}</button><button class="turn-icon" type="button" data-turn-act="rewind" data-turn="${escape(entry.turn!)}" data-key="${entry.key}" data-needs="rewind" aria-label="${escape(words.rewind)}" title="${escape(words.rewind)}">${glyph("edit", 16)}</button></div>`;
  }

  /** A notice as one quiet line on the right, a mark before it; what it
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
        said = `${notice.label} · ${notice.health.text}`;
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
    return `<div class="notice-line${ok ? "" : " failed"}"><button class="notice-open" type="button" data-open="${entry.key}" aria-expanded="${open}">${line}${glyph("chevron", 14)}</button></div><div class="fold-box notice-box"${open ? "" : ' style="height:0" inert'}><div class="fold-content"></div></div>`;
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

  /** A stretch's element, made once and brought up to date in place: its
   *  heading's words, and its rows, each drawn again only when it changed. */
  private stretch(group: Activity): Stretch {
    let stretch = this.stretches.get(group.key);
    if (!stretch) {
      const element = document.createElement("div");
      element.className = "block block-activity";
      element.dataset.key = group.key;
      const open = this.openStretches.has(group.key);
      element.innerHTML = `<button class="group-head" type="button" data-fold="${group.key}" aria-expanded="${open}"><span class="group-title"></span><span class="row-stats group-stats"></span><span class="group-time"></span>${glyph("chevron", 16)}</button><div class="fold-box group-box"${open ? "" : ' style="height:0" inert'}><div class="rows"></div></div>`;
      stretch = {
        element,
        head: element.querySelector<HTMLButtonElement>(".group-head")!,
        box: element.querySelector<HTMLElement>(".group-box")!,
        rows: element.querySelector<HTMLElement>(".rows")!,
        drawn: new Map(),
      };
      this.stretches.set(group.key, stretch);
    }
    this.heading(stretch, group);
    const members = group.members.filter(visible);
    const present = new Set<string>();
    let previous: HTMLElement | null = null;
    for (const member of members) {
      present.add(member.key);
      const signature = this.memberSignature(member);
      let drawn = stretch.drawn.get(member.key);
      if (!drawn || drawn.signature !== signature) {
        const holder = document.createElement("div");
        holder.innerHTML = this.member(member);
        const fresh = holder.firstElementChild as HTMLElement;
        if (drawn) {
          this.letGo(member.key);
          drawn.element.replaceWith(fresh);
        }
        if (this.openRows.has(member.key)) this.fill(member.key, fresh);
        drawn = { signature, element: fresh };
        stretch.drawn.set(member.key, drawn);
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
    return stretch;
  }

  /** A stretch's heading: while it runs, the step it is on, how many steps
   *  and for how long; once done, what it did, the lines it changed, and
   *  how long it took. The words change in place; the line keeps its
   *  height. */
  private heading(stretch: Stretch, group: Activity): void {
    const { words } = this.context;
    const tools = group.members.filter((member): member is ToolEntry => member.kind === "tool");
    const thinking = group.members.filter((member) => member.kind === "prose");
    const steps = group.members.filter(visible).length;
    const writing = thinking.find((member) => member.kind === "prose" && member.writing);
    const runningTool = tools.find((tool) => tool.ended === null);
    const running = !!runningTool || !!writing;
    let title: string;
    let stats = "";
    if (runningTool) {
      const line = headline(runningTool.tool, null, words, (path) => this.conversation.attachmentName(path));
      title = [line.action, line.object].filter(Boolean).join(" ");
    } else if (writing && writing.kind === "prose") {
      const titles = [...writing.text.matchAll(/^\*\*(.+?)\*\*\s*$/gm)].map((match) => match[1]);
      const last = titles[titles.length - 1];
      title = last ? `${words.thought}：${last}` : words.thinking;
    } else {
      title = groupHeading(tools, thinking.length, words);
      let added = 0;
      let removed = 0;
      for (const tool of tools) {
        if (tool.tool.does.as !== "edit") continue;
        for (const change of tool.tool.does.changes) {
          const counted = counts(change.diff);
          added += counted.added;
          removed += counted.removed;
        }
      }
      stats = added || removed ? `<span class="add">+${added}</span><span class="del">−${removed}</span>` : "";
    }
    const began = tools.length ? Math.min(...tools.map((tool) => tool.began)) : null;
    const ended = !running && tools.length && tools.every((tool) => tool.ended) ? Math.max(...tools.map((tool) => tool.ended!.at)) : null;
    const titleElement = stretch.head.querySelector<HTMLElement>(".group-title")!;
    if (titleElement.textContent !== title) titleElement.textContent = title;
    const statsElement = stretch.head.querySelector<HTMLElement>(".group-stats")!;
    if (statsElement.innerHTML !== stats) statsElement.innerHTML = stats;
    stretch.head.classList.toggle("going", running);
    const time = stretch.head.querySelector<HTMLElement>(".group-time")!;
    if (running && began !== null) {
      time.dataset.since = String(began);
      time.dataset.steps = String(steps);
    } else {
      delete time.dataset.since;
      const said = [running || steps < 2 ? "" : words.steps(steps), began !== null && ended !== null ? words.took(ended - began) : ""].filter(Boolean).join(" · ");
      if (time.textContent !== said) time.textContent = said;
    }
  }

  /** Brings the running stretches' clocks, and the turn's, up to date,
   *  once a second while any runs. */
  private tick(): void {
    const clocks = [...this.root.querySelectorAll<HTMLElement>(".group-time[data-since]")];
    const { words } = this.context;
    for (const clock of clocks) {
      const said = [words.steps(Number(clock.dataset.steps)), words.took(Date.now() - Number(clock.dataset.since))].join(" · ");
      if (clock.textContent !== said) clock.textContent = said;
    }
    const turn = this.status.isConnected ? this.status.querySelector<HTMLElement>(".turn-status-time[data-since]") : null;
    if (turn) {
      const said = words.took(Date.now() - Number(turn.dataset.since));
      if (turn.textContent !== said) turn.textContent = said;
    }
    clearTimeout(this.ticking);
    if (clocks.length || turn) this.ticking = window.setTimeout(() => this.tick(), 1000);
  }

  /** The turn's line: what it is doing now and since when, or that it
   *  waits on the user; none when no turn runs. */
  private drawStatus(): boolean {
    const { words } = this.context;
    const waiting = this.conversation.waiting();
    const busy = this.conversation.busy();
    if (!waiting && !busy) return false;
    const doing = this.conversation.doing();
    let step = words.working;
    if (waiting) step = words.waitingForYou;
    else if (doing?.kind === "tool") {
      const line = headline(doing.tool, null, words, (path) => this.conversation.attachmentName(path));
      step = [line.action, line.object].filter(Boolean).join(" ");
    } else if (doing?.kind === "prose" && doing.prose === "thinking") {
      const titles = [...doing.text.matchAll(/^\*\*(.+?)\*\*\s*$/gm)].map((match) => match[1]);
      step = titles.length ? `${words.thought}：${titles[titles.length - 1]}` : words.thinking;
    } else if (doing?.kind === "prose") step = words.answering;
    this.status.classList.toggle("waiting", waiting);
    const stepElement = this.status.querySelector<HTMLElement>(".turn-status-step")!;
    if (stepElement.textContent !== step) stepElement.textContent = step;
    const time = this.status.querySelector<HTMLElement>(".turn-status-time")!;
    const began = this.conversation.turnBegan();
    if (began !== null && !waiting) time.dataset.since = String(began);
    else {
      delete time.dataset.since;
      time.textContent = "";
    }
    return true;
  }

  private member(entry: Entry): string {
    const enter = this.settled && !this.seen.has(entry.key) ? " entering" : "";
    this.seen.add(entry.key);
    if (entry.kind === "prose") return this.thinking(entry, enter);
    if (entry.kind === "activity") {
      return `<div class="row${enter}" data-row="${entry.key}"><span class="rail">${glyph("colleague")}</span><div class="row-line static"><span class="row-object">${escape(entry.text)}</span></div></div>`;
    }
    if (entry.kind !== "tool") return `<div data-row="${entry.key}"></div>`;
    const { words } = this.context;
    const line = headline(entry.tool, entry.ended, words, (path) => this.conversation.attachmentName(path));
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
    const fold = expandable ? `<div class="fold-box"${open ? "" : ' style="height:0" inert'}><div class="fold-content"></div></div>` : "";
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
    return `<div class="row row-tool row-${tense}${enter}" data-row="${entry.key}"><span class="rail">${mark}</span><button class="row-line" type="button" ${expandable ? `data-open="${entry.key}"` : "disabled"} aria-expanded="${open}"><span class="row-action">${escape(line.action)}</span>${object}${stats}${this.outcome(entry)}${elsewhere}${chevron}</button>${colleague}${sub}${fold}</div>`;
  }

  private thinking(entry: Extract<Entry, { kind: "prose" }>, enter: string): string {
    const { words } = this.context;
    // Its words are shown only where the host gave them; none are made up.
    const empty = !entry.text.trim();
    const open = this.openRows.has(entry.key);
    // A summary of thinking comes in sections, each under a bold title of
    // the host's: the row says the latest while it thinks, the first after.
    const titles = [...entry.text.matchAll(/^\*\*(.+?)\*\*\s*$/gm)].map((match) => match[1]);
    const title = entry.writing ? titles[titles.length - 1] : titles[0];
    const label = entry.writing ? words.thinking : empty ? words.thoughtWithheld : words.thought;
    const said = title
      ? `<span class="row-action">${escape(label)}</span><span class="row-object">${escape(title)}</span>`
      : `<span class="row-action">${escape(label)}</span>`;
    const chevron = empty ? "" : `<span class="row-chevron">${glyph("chevron", 16)}</span>`;
    const fold = empty ? "" : `<div class="fold-box"${open ? "" : ' style="height:0" inert'}><div class="fold-content"></div></div>`;
    return `<div class="row row-thinking${entry.writing ? " row-running" : ""}${empty ? " withheld" : ""}${enter}" data-row="${entry.key}"><span class="rail">${glyph("thinking")}</span><button class="row-line" type="button" ${empty ? "disabled" : `data-open="${entry.key}"`} aria-expanded="${open}">${said}${chevron}</button>${fold}</div>`;
  }

  private outcome(entry: ToolEntry): string {
    const { words } = this.context;
    const { tool } = entry;
    if (tool.does.as === "command") {
      if (tool.does.job) return `<span class="row-outcome went-on">${escape(words.wentOnAs(tool.does.job))}</span>`;
      if (tool.does.exit !== null && tool.does.exit !== 0) {
        return `<span class="row-outcome wrong">${escape(words.exited(tool.does.exit))}</span>`;
      }
    }
    switch (entry.ended?.use) {
      case "cut":
        return `<span class="row-outcome">${escape(words.cut)}</span>`;
      case "unknown":
        return `<span class="row-outcome">${escape(words.unknown)}</span>`;
      case "declined":
        return `<span class="row-outcome wrong">${escape(words.declined)}</span>`;
      default:
        return "";
    }
  }

  private hasDetails(tool: Tool): boolean {
    if (tool.does.as === "edit") return tool.does.changes.length > 0;
    if (tool.does.as === "command") return true;
    const given = tool.input !== null && tool.input !== undefined && JSON.stringify(tool.input) !== "{}";
    return given || tool.output.length > 0;
  }

  private longWords() {
    const { words, english } = this.context;
    return { more: words.moreLines, less: words.showLess, full: words.fullScreen, english };
  }

  /** Fills what opens under a row or a notice: a change as a diff, a
   *  command's words and what it printed, anything else's input and output,
   *  thinking's words. */
  private fill(key: string, row: HTMLElement): void {
    const content = row.querySelector<HTMLElement>(":scope > .fold-box > .fold-content") ?? row.querySelector<HTMLElement>(".fold-box > .fold-content");
    if (!content) return;
    this.letGo(key);
    content.replaceChildren();
    const entry = this.conversation.entries.find((candidate) => candidate.key === key);
    if (!entry) return;
    if (entry.kind === "heard") {
      this.noticeDetails(entry, content);
      return;
    }
    if (entry.kind === "prose") {
      content.innerHTML = `<div class="thinking-text prose">${render(entry.text)}</div>`;
      return;
    }
    if (entry.kind !== "tool") return;
    const { words } = this.context;
    const { tool } = entry;
    if (tool.does.as === "edit") {
      content.innerHTML = tool.does.changes
        .map((change) => {
          const rows = parse(change.diff);
          const place = numbered(rows) ? "" : " unnumbered";
          return `<div class="surface diff"><div class="surface-head"><span class="surface-title" title="${escape(change.path)}">${escape(change.path)}</span></div><div class="diff-rows${place}">${renderRows(rows, change.path, words.unchangedLines)}</div></div>`;
        })
        .join("");
      return;
    }
    const output = textOf(tool.output);
    const images = imagesOf(tool.output);
    const given =
      tool.does.as === "command"
        ? tool.does.command
        : tool.input !== null && tool.input !== undefined && JSON.stringify(tool.input) !== "{}"
          ? JSON.stringify(tool.input, null, 2)
          : "";
    const surface = document.createElement("div");
    surface.className = "surface io";
    const longs: LongText[] = [];
    const part = (label: string, text: string, extra = "") => {
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
    if (given) part("IN", given);
    if (output || images) part("OUT", output, images);
    content.append(surface);
    this.longs.set(key, longs);
  }

  /** Lets go of what a row held open. */
  private letGo(key: string): void {
    for (const long of this.longs.get(key) ?? []) long.destroy();
    this.longs.delete(key);
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
   *  closes. Open, it takes the height of what is in it as that grows. */
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
          ? this.conversation.entries.find((candidate) => candidate.key === target.dataset.key)
          : this.conversation.entries.find((candidate) => candidate.kind === "heard" && candidate.turn === turn && (!candidate.from || candidate.from.kind === "user"));
      this.context.turnAct?.(target.dataset.turnAct as "rewind" | "regenerate" | "fork", turn, entry?.kind === "heard" ? entry.text : null, entry?.kind === "heard" ? (entry.kept ?? []) : []);
      return;
    }
    if (target.dataset.copy || target.dataset.copyTurn) {
      // A message is copied as it was written, in its Markdown; a turn's
      // answer as all it said.
      const entry = this.conversation.entries.find((candidate) => candidate.key === target.dataset.copy);
      const text = target.dataset.copyTurn
        ? this.conversation.entries
            .filter((candidate): candidate is Extract<Entry, { kind: "prose" }> => candidate.kind === "prose" && candidate.prose === "message" && candidate.turn === target.dataset.copyTurn)
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
      const stretch = this.stretches.get(key);
      if (!stretch) return;
      const open = !this.openStretches.has(key);
      if (open) this.openStretches.add(key);
      else this.openStretches.delete(key);
      target.setAttribute("aria-expanded", String(open));
      void this.fold(stretch.box, target, open, () => {});
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

function textOf(output: Content[]): string {
  return output
    .filter((content): content is Extract<Content, { type: "text" }> => content.type === "text")
    .map((content) => content.text)
    .join("\n");
}

function imagesOf(output: Content[]): string {
  return output
    .map((content) => {
      if (content.type === "image") return `<img class="io-image" data-picture alt="" src="data:${escape(content.mime)};base64,${content.base64}">`;
      if (content.type === "image_url") return `<img class="io-image" data-picture alt="" src="${escape(content.url)}">`;
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
