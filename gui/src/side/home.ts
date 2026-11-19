// The side's first page (`world-gui.md`, 对话的侧面板): the conversation in
// focus at a glance, so the most is seen for the least done. A card for each
// of its parts with anything to say, each folding away under its name: the
// goal, what it is and how it stands; the plan, read whole in its card,
// which takes what height the others leave; the files it changed; the
// colleagues it asked and the host's agents it sent, those at work first.
// Pressing what a card shows opens it: the goal as a page of the side, a
// file on a tab of it, a colleague beside the conversation, one of the
// host's agents at the row that sent it. The history is no card: what is
// kept to look things up in has its own mark in the desk's corner. A card is
// a thing of its own: taken by its head it is carried to another place among
// the cards, or out of the side, where it is let go into a window of its own
// (`board.ts`), as the mark at its head's end does at a press; its place is
// kept in the side meanwhile, and from there it is called forward or put
// back.
//
// A card shows all of what it holds, scrolling in the card past a height:
// nothing of it is left out to be asked for, and nothing opens over the side
// (the user, 2026-11-17: "全部搞成能滚动的，就没有展开逻辑了"). The files' card
// alone has two things to show, the files changed a line each, or their
// page, the list in full and the folders it works in: a mark at its head
// turns it from the one to the other where it stands, the one folding as the
// other opens ("可以是一个图标"). In a window of its own a card fills the
// window, with none of what folds it or takes it out.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { figures, mark } from "../files/changed";
import { tally, type ChangedFile } from "../files/model";
import type { FilesWords } from "../files/words";
import type { PlanShape } from "../plan/shape";
import { going } from "./collab-page";
import type { SidePage } from "./pane";
import { ONLY, onBoards } from "./board";
import { cardHidden, cardNames, cardOrder, moveCard, onCardsChosen, openCardsMenu, type Card } from "./shown";
import type { CollabNode, GoalShown } from "./types";
import type { SideWords } from "./words";

/** Where this device keeps the cards folded away, and whether the files'
 *  card shows their page. */
const FOLDED = "side-home-folded";
const BROWSING = "side-files-browsing";

export interface HomeSeams {
  /** The goal's page of the side. */
  page(part: "goal"): void;
  /** One of the files it changed, opened to its changes. */
  file?(machine: string, path: string): void;
  /** Whether a card stands in a window of its own, its place kept here. */
  away?(card: Card): boolean;
  /** Calls a card's window forward; asks a card back out of its window. */
  front?(card: Card): void;
  back?(card: Card): void;
  /** Takes a card out into a window of its own, from where it stands on
   *  the screen; none where that cannot be. */
  pull?: ((card: Card, from: { left: number; top: number; width: number; height: number }) => void) | undefined;
}

/** How far a card's head is moved before it is being carried, not pressed;
 *  and how long after it is let go a press is taken for the end of that. */
const CARRIED = 6;
const SETTLE = 400;

function kept(key: string): Set<string> {
  try {
    return new Set(JSON.parse(localStorage.getItem(key) ?? "[]") as string[]);
  } catch {
    return new Set();
  }
}

function keep(key: string, kept: string): void {
  try {
    localStorage.setItem(key, kept);
  } catch {
    // This device's convenience; without storage it lasts the page.
  }
}

/** What a card says and has besides its name, how it stands and what it
 *  holds. */
interface CardSays {
  /** What waits on the user, said in clay after how it stands. */
  waiting?: string;
  /** How it stands is itself what waits on the user. */
  waits?: boolean;
  /** The goal's bar above the input says it, so it stands only where the
   *  side lies over the conversation. */
  barred?: boolean;
  /** Set after how it stands, as written. */
  tail?: string;
  /** The marks at the head's end, before the one that takes the card out. */
  marks?: string;
  /** What it says, where that is not all of the above: what is turned
   *  where the card stands is no reason to set it anew. */
  sign?: string;
}

/** Everything in `within` that has been scrolled, and how far. */
function scrolled(within: HTMLElement): [HTMLElement, number][] {
  return [within, ...within.querySelectorAll<HTMLElement>("*")].filter((one) => one.scrollTop > 0).map((one) => [one, one.scrollTop]);
}

/** Carries the plan's card from the height it stands at down to its name
 *  as it shuts. Its plan is taller than the room the card has, so folded by
 *  its rows alone the card stood at that height, emptying, until the rows
 *  had fallen below it, and only then came down: the plan gone from a card
 *  still standing. */
function shutFrom(card: HTMLElement, head: HTMLElement, from: number): void {
  const { duration, easing } = pace(card);
  const to = head.getBoundingClientRect().height;
  if (duration <= 0 || from <= to + 1) return;
  card.animate([{ height: `${from}px` }, { height: `${to}px` }], { duration, easing });
}

/** The pace and curve a card folds at, as the design's tokens have them
 *  where it stands: no time at all where motion is reduced. */
function pace(card: HTMLElement): { duration: number; easing: string } {
  const style = getComputedStyle(card);
  const token = style.getPropertyValue("--expand").trim();
  return { duration: parseFloat(token) * (token.endsWith("ms") ? 1 : 1000) || 0, easing: style.getPropertyValue("--expand-easing").trim() || "ease" };
}

export class SideHome implements SidePage {
  readonly element: HTMLElement;
  private goalShown: GoalShown | null = null;
  /** The goal stands on its bar above the input. */
  private barred = false;
  private planShown: PlanShape | null = null;
  private planned = false;
  private nodes: CollabNode[] = [];
  private changed: ChangedFile[] = [];
  private readonly fold = kept(FOLDED);
  /** The files' card shows their page, not the files a line each. */
  private browsing = false;
  /** The cards on the page, by what they are of, each with what it said
   *  when it was set; what each says now, as it was last made; and the
   *  line said where no card stands. */
  private readonly standing = new Map<string, { sign: string; element: HTMLElement }>();
  private readonly signs = new Map<string, string>();
  private readonly none = document.createElement("p");
  /** What it hears of all the conversations' sides, each to be stopped. */
  private readonly unheard: (() => void)[] = [];
  /** When a card was last let go after being carried. */
  private carriedAt = -SETTLE;

  /** `plan` is the plan's own page, drawn in its card. */
  constructor(
    private readonly words: SideWords,
    private readonly seams: HomeSeams,
    private readonly goals: boolean,
    private readonly planPage: HTMLElement,
    /** The words of the files it changed; none where they are not shown. */
    private readonly fileWords: FilesWords | null = null,
    /** The pages set into their cards: who works with the conversation, a
     *  page to a card; and the files' own, which their card turns to. */
    private readonly pages: Partial<Record<Card, SidePage>> = {},
  ) {
    try {
      this.browsing = localStorage.getItem(BROWSING) === "1";
    } catch {
      // Without storage the card starts at its lines.
    }
    this.element = document.createElement("section");
    this.element.className = "sh-page";
    this.none.className = "sh-none";
    this.element.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      // The press that ends a card's being carried does nothing more; and
      // a page set into a card hears its own presses.
      if (performance.now() - this.carriedAt < SETTLE || target.closest(".sh-frame")) return;
      const browse = target.closest<HTMLElement>("[data-browse]");
      if (browse) return this.browse(browse);
      const pulled = target.closest<HTMLElement>("[data-pull]");
      if (pulled) {
        const section = pulled.closest<HTMLElement>(".sh-card");
        if (section) this.seams.pull?.(pulled.dataset.pull as Card, section.getBoundingClientRect());
        return;
      }
      const back = target.closest<HTMLElement>("[data-back]")?.dataset.back as Card | undefined;
      if (back) return this.seams.back?.(back);
      const front = target.closest<HTMLElement>("[data-front]")?.dataset.front as Card | undefined;
      if (front) return this.seams.front?.(front);
      const head = target.closest<HTMLElement>(".sh-head");
      // Alone in its window a card does not fold: there is nothing else
      // for it to make room for.
      if (head) return ONLY ? undefined : this.turn(head);
      const open = target.closest<HTMLElement>("[data-open]")?.dataset.open;
      if (open === "goal") return this.seams.page(open);
      const file = target.closest<HTMLElement>("[data-file]");
      if (file) this.seams.file?.(file.dataset.machine!, file.dataset.file!);
    });
    this.element.addEventListener("pointerdown", (event) => this.take(event));
    // Pressed with the other button, the overview offers its cards to be
    // shown or put away (`shown.ts`); words chosen in it keep the system's
    // own menu, to be copied.
    this.element.addEventListener("contextmenu", (event) => {
      if (getSelection()?.toString() || (event.target as HTMLElement).closest("a, input, textarea")) return;
      event.preventDefault();
      if (ONLY) return;
      // On a card, the menu first offers to take that card out.
      const section = (event.target as HTMLElement).closest<HTMLElement>(".sh-card:not(.sh-away)");
      const card = section?.dataset.of as Card | undefined;
      // Not where the side lies over the whole desk, as on a phone.
      const pull = this.element.closest(".sp-over") ? undefined : this.seams.pull;
      openCardsMenu(event.clientX, event.clientY, this.words, pull && section && card && card !== "goal" ? () => pull(card, section.getBoundingClientRect()) : undefined);
    });
    // One overview hears what is chosen for all of them, and stops hearing
    // as its conversation is let go (`dispose`): what hears holds it.
    this.unheard.push(onCardsChosen(() => this.draw()));
    // A card taken out leaves the overview, and is back as its window goes.
    this.unheard.push(onBoards(() => this.draw()));
    this.draw();
  }

  /** Its conversation is let go. */
  dispose(): void {
    for (const stop of this.unheard.splice(0)) stop();
  }

  /** Folds a card away under its name, or opens it again, where it stands:
   *  it eases shut or open as a conversation's steps do, and the next
   *  drawing finds it as it is. */
  private turn(head: HTMLElement): void {
    const card = head.dataset.card!;
    const shut = !this.fold.has(card);
    if (shut) this.fold.add(card);
    else this.fold.delete(card);
    keep(FOLDED, JSON.stringify([...this.fold]));
    const section = head.closest<HTMLElement>(".sh-card");
    // How tall the plan's card stands as it is shut, before it changes.
    const from = card === "plan" && shut && section ? section.getBoundingClientRect().height : 0;
    section?.classList.toggle("shut", shut);
    section?.querySelector(":scope > .sh-fold")?.classList.toggle("open", !shut);
    head.setAttribute("aria-expanded", String(!shut));
    if (from && section) shutFrom(section, head.parentElement ?? head, from);
  }

  /** Turns the files' card from the files a line each to their page, or
   *  back, where it stands, with nothing set anew: the card goes from the
   *  one height to the other while the one left fades out over the one
   *  coming in, both from the card's top (the user, 2026-11-17: "缺乏自然
   *  的切换动画"). Turned again before it is done, it goes on from where it
   *  is. A card folded away is opened to what it was turned to. */
  private browse(mark: HTMLElement): void {
    const section = mark.closest<HTMLElement>(".sh-card");
    const views = section?.querySelector<HTMLElement>(".sh-views");
    if (!section || !views) return;
    this.browsing = !this.browsing;
    keep(BROWSING, this.browsing ? "1" : "");
    if (this.browsing) this.pages.files?.shown?.();
    const from = views.getBoundingClientRect().height;
    for (const going of views.getAnimations({ subtree: true })) going.cancel();
    let left: HTMLElement | null = null;
    let came: HTMLElement | null = null;
    for (const view of views.querySelectorAll<HTMLElement>(":scope > .sh-view")) {
      const shown = (view.dataset.view === "page") === this.browsing;
      view.classList.toggle("shown", shown);
      view.classList.toggle("leaving", !shown);
      if (shown) came = view;
      else left = view;
    }
    const { duration, easing } = pace(section);
    const folded = section.classList.contains("shut");
    if (left && came && duration > 0 && !folded) {
      // The height it comes to is that of the one coming in alone.
      left.classList.remove("leaving");
      const to = views.getBoundingClientRect().height;
      left.classList.add("leaving");
      const leaving = left;
      views.animate([{ height: `${from}px` }, { height: `${to}px` }], { duration, easing });
      came.animate([{ opacity: 0 }, { opacity: 1 }], { duration, easing });
      // Gone once it has faded; one turned back to meanwhile is no longer
      // leaving, and its fading was called off.
      leaving.animate([{ opacity: 1 }, { opacity: 0 }], { duration, easing }).finished.then(
        () => leaving.classList.remove("leaving"),
        () => {},
      );
    } else left?.classList.remove("leaving");
    // The mark says what it now does, and stays the one the hand is on.
    const { said, drawn } = this.browseSays();
    mark.setAttribute("aria-pressed", String(this.browsing));
    mark.setAttribute("aria-label", said);
    mark.title = said;
    mark.innerHTML = drawn;
    const head = section.querySelector<HTMLElement>(".sh-head");
    if (head && section.classList.contains("shut")) this.turn(head);
  }

  /** What the mark that turns the files' card says and is drawn as: to
   *  their page, or back to the files a line each. */
  private browseSays(): { said: string; drawn: string } {
    const side = this.fileWords!.side;
    return { said: this.browsing ? side.lines : side.whole, drawn: glyph(this.browsing ? "fold" : "unfold", 16) };
  }

  private browseMark(): string {
    const { said, drawn } = this.browseSays();
    return `<button class="icon-button sh-mark sh-browse" type="button" data-browse aria-pressed="${this.browsing}" aria-label="${escape(said)}" title="${escape(said)}">${drawn}</button>`;
  }

  /** `barred`: the goal stands on its bar above the input. */
  goal(goal: GoalShown | null, barred: boolean): void {
    this.goalShown = goal;
    this.barred = barred;
    this.draw();
  }

  plan(source: string | null, shape: PlanShape | null): void {
    // A plan takes whatever form its work asks for: any text is a plan.
    this.planned = !!source?.trim();
    this.planShown = shape;
    this.draw();
  }

  team(nodes: CollabNode[]): void {
    this.nodes = nodes;
    this.draw();
  }

  /** The files the conversation changed, as they now stand. */
  files(files: ChangedFile[]): void {
    const sign = (all: ChangedFile[]) => all.map((file) => `${file.key}|${file.how}|${file.added}|${file.removed}|${file.changes[0]?.at ?? 0}`).join("\n");
    if (sign(files) === sign(this.changed)) return;
    this.changed = files;
    this.draw();
  }

  private draw(): void {
    // Each card the user has not put away, where it has anything to say.
    const made: Record<Exclude<Card, "goal">, () => string> = {
      plan: () => this.planCard(),
      files: () => this.filesCard(),
      colleagues: () => this.teamCard("colleagues", this.nodes.filter((node) => node.kind === "colleague" || node.kind === "side")),
      agents: () => this.teamCard("agents", this.nodes.filter((node) => node.kind === "subagent")),
    };
    // In the order the user put them. In a window of its own a card stands
    // alone, put away or not; one taken out keeps its place here, said in
    // a line.
    const chosen = cardOrder().filter((card): card is Exclude<Card, "goal"> => card !== "goal" && (ONLY ? card === ONLY : !cardHidden(card)));
    const names = cardNames(this.words);
    const cards = [ONLY ? "" : this.goalCard(Date.now()), ...chosen.map((card) => (!ONLY && this.seams.away?.(card) ? this.awayCard(card, names[card]) : made[card]()))].filter(Boolean);
    // Said where no card stands, the goal's left to its bar among them; and
    // where they were all put away, how they are brought back.
    const none = chosen.length ? this.words.home.none : this.words.home.hidden;
    this.put(cards);
    if (this.none.textContent !== none) this.none.textContent = none;
    this.element.classList.toggle("sh-has-cards", cards.length > 0);
    this.element.classList.toggle("sh-has-unbarred", this.element.querySelector(".sh-card:not(.barred)") !== null);
  }

  /** Puts the cards on the page, changing only what changed: a card whose
   *  words are what they were is left standing as it is, folded or open,
   *  mid-fold or scrolled; one whose words changed is set anew in its
   *  place. The whole page used to be written again at every change (a
   *  helper's minute passing among them), which took the plan's page off
   *  the page and put it back, and a page put back is scrolled to its top:
   *  the plan came back to its beginning under the reader. Where a card
   *  itself must be set anew, the page it holds is moved into the new card,
   *  and what was scrolled in the card is left where it was. */
  private put(cards: string[]): void {
    const top = this.planPage.scrollTop;
    const wanted = new Map<string, string>();
    for (const html of cards) wanted.set(/ data-of="([a-z]+)"/.exec(html)?.[1] ?? html, html);
    for (const [key, card] of this.standing) {
      if (wanted.has(key)) continue;
      card.element.remove();
      this.standing.delete(key);
    }
    let previous: Element | null = null;
    for (const [key, html] of wanted) {
      const sign = this.signs.get(key) ?? html;
      let card = this.standing.get(key);
      if (!card || card.sign !== sign) {
        const made = document.createElement("template");
        made.innerHTML = html;
        const element = made.content.firstElementChild as HTMLElement;
        // The plan's page is one element, kept as its card is set anew; so
        // is the page set into a card. A page moved is scrolled back to its
        // top, and so are the lines written again: both are put back where
        // they stood.
        if (key === "plan") element.querySelector(".sh-plan")?.append(this.planPage);
        const slot = element.querySelector<HTMLElement>(".sh-frame");
        const page = slot ? this.pages[key as Card] : undefined;
        const stood = card && page ? scrolled(page.element) : [];
        const lines = card?.element.querySelector(".sh-files")?.scrollTop ?? 0;
        if (slot && page) {
          slot.append(page.element);
          // The files' page draws what it holds as it is turned to.
          if (key !== "files" || this.browsing) page.shown?.();
        }
        if (card) card.element.replaceWith(element);
        for (const [one, at] of stood) one.scrollTop = at;
        const listed = element.querySelector(".sh-files");
        if (listed && lines) listed.scrollTop = lines;
        card = { sign, element };
        this.standing.set(key, card);
      }
      const place: Element | null = previous ? previous.nextElementSibling : this.element.firstElementChild;
      if (card.element !== place) this.element.insertBefore(card.element, place);
      previous = card.element;
    }
    if (this.element.lastElementChild !== this.none) this.element.append(this.none);
    if (this.planPage.scrollTop !== top) this.planPage.scrollTop = top;
  }

  /** A card under its name, folding away when the name is pressed; how it
   *  stands after the name. */
  private card(card: Card, name: string, state: string, body: string, says: CardSays = {}): string {
    const { waiting = "", waits = false, barred = false, tail = "", marks = "" } = says;
    // What it says, apart from whether it is folded: folding is done where
    // the card stands, and is no reason to set it anew.
    this.signs.set(card, says.sign ?? [name, state, body, waiting, waits, barred, tail, marks].join("\u0001"));
    const shut = !ONLY && this.fold.has(card);
    const said = `${escape(state)}${tail}${waiting ? `<span class="sh-waits">${escape(waiting)}</span>` : ""}`;
    // At the head's end, the mark that takes the card out into a window of
    // its own, where that can be.
    const out = this.words.board.out;
    const pull = !ONLY && card !== "goal" && this.seams.pull ? `<button class="icon-button sh-mark sh-pull" type="button" data-pull="${card}" aria-label="${escape(out)}" title="${escape(out)}">${glyph("dock-out", 16)}</button>` : "";
    return `<section class="sh-card${shut ? " shut" : ""}${barred ? " barred" : ""}" data-of="${card}"><div class="sh-top"><button class="sh-head" type="button" data-card="${card}" aria-expanded="${!shut}"><span class="sh-name">${escape(name)}</span><span class="sh-turn">${glyph("chevron", 12)}</span><span class="sh-state${waits ? " waiting" : ""}">${said}</span></button>${marks}${pull}</div><div class="sh-fold${shut ? "" : " open"}"><div class="sh-fold-inner"><div class="sh-body">${body}</div></div></div></section>`;
  }

  /** The place of a card that stands in a window of its own: its name and
   *  where it is, in a line; pressed, its window comes forward; the mark at
   *  the line's end puts it back. */
  private awayCard(card: Card, name: string): string {
    const words = this.words;
    this.signs.set(card, "away");
    return `<section class="sh-card sh-away" data-of="${card}"><div class="sh-top"><button class="sh-head" type="button" data-front="${card}" title="${escape(words.board.front)}"><span class="sh-name">${escape(name)}</span><span class="sh-state">${escape(words.home.away)}</span></button><button class="icon-button sh-mark sh-pull" type="button" data-back="${card}" aria-label="${escape(words.board.back)}" title="${escape(words.board.back)}">${glyph("dock-right", 16)}</button></div></section>`;
  }

  /** A card taken by its head and carried: among the cards, to the place
   *  a line marks, where it then stands on every overview; or out of the
   *  side, where it is let go into a window of its own, there. A head only
   *  pressed folds its card as before. Not by a finger, which scrolls the
   *  page; nor where the side lies over the whole desk. */
  private take(event: PointerEvent): void {
    if (ONLY || event.button !== 0 || event.pointerType === "touch") return;
    const target = event.target as HTMLElement;
    const top = target.closest<HTMLElement>(".sh-top");
    const section = top?.closest<HTMLElement>(".sh-card");
    const card = section?.dataset.of as Card | undefined;
    if (!top || !section || !card || card === "goal" || target.closest(".sh-mark") || this.element.closest(".sp-over")) return;
    const panel = this.element.closest<HTMLElement>(".sp-panel") ?? this.element;
    const from = section.getBoundingClientRect();
    const grab = { x: event.clientX - from.left, y: event.clientY - from.top };
    const pull = section.classList.contains("sh-away") ? undefined : this.seams.pull;
    let ghost: HTMLElement | null = null;
    let line: HTMLElement | null = null;
    /** The card it would stand before, none for last; undefined while it
     *  is carried outside the side. */
    let before: Card | null | undefined;
    const move = (at: PointerEvent) => {
      if (!ghost) {
        if (Math.hypot(at.clientX - event.clientX, at.clientY - event.clientY) < CARRIED) return;
        // Held, so it is followed past the window's edge too.
        top.setPointerCapture(event.pointerId);
        ghost = top.cloneNode(true) as HTMLElement;
        ghost.className = "sh-ghost";
        ghost.style.width = `${from.width}px`;
        line = document.createElement("div");
        line.className = "sh-drop";
        document.body.append(ghost, line);
        section.classList.add("lifted");
        document.documentElement.classList.add("sh-carrying");
      }
      ghost.style.transform = `translate(${at.clientX - grab.x}px, ${at.clientY - grab.y}px)`;
      const box = panel.getBoundingClientRect();
      const inside = at.clientX >= box.left && at.clientX <= box.right && at.clientY >= box.top && at.clientY <= box.bottom;
      ghost.classList.toggle("leaving", !inside && !!pull);
      before = undefined;
      line!.hidden = true;
      if (!inside) return;
      // Before the first card whose middle is under the pointer, else last.
      const cards = [...this.element.querySelectorAll<HTMLElement>(".sh-card")].filter((one) => one.dataset.of !== "goal");
      const next = cards.find((one) => {
        const rect = one.getBoundingClientRect();
        return at.clientY < rect.top + rect.height / 2;
      });
      before = (next?.dataset.of as Card | undefined) ?? null;
      // Where it already stands, no line is drawn.
      const at0 = cards.indexOf(section);
      if (next === section || (next ? cards.indexOf(next) : cards.length) === at0 + 1) return;
      const page = this.element.getBoundingClientRect();
      const edge = next ? next.getBoundingClientRect().top - 4 : cards[cards.length - 1]!.getBoundingClientRect().bottom + 4;
      line!.hidden = false;
      line!.style.cssText = `left:${page.left + 8}px;width:${page.width - 16}px;top:${Math.round(edge) - 1}px`;
    };
    const stop = () => {
      removeEventListener("pointermove", move, true);
      removeEventListener("pointerup", up, true);
      removeEventListener("pointercancel", stop, true);
      removeEventListener("keydown", key, true);
      if (!ghost) return;
      ghost.remove();
      line?.remove();
      section.classList.remove("lifted");
      document.documentElement.classList.remove("sh-carrying");
      if (top.hasPointerCapture(event.pointerId)) top.releasePointerCapture(event.pointerId);
      this.carriedAt = performance.now();
    };
    const up = (at: PointerEvent) => {
      const carried = ghost !== null;
      const leaving = ghost?.classList.contains("leaving") ?? false;
      stop();
      if (!carried) return;
      if (before !== undefined) moveCard(card, before === card ? null : before);
      else if (leaving) pull?.(card, { left: at.clientX - grab.x, top: at.clientY - grab.y, width: from.width, height: from.height });
    };
    const key = (pressed: KeyboardEvent) => {
      if (pressed.key === "Escape") stop();
    };
    // Heard on the window: a quick hand is past the head before it has
    // moved far enough to be carrying.
    addEventListener("pointermove", move, true);
    addEventListener("pointerup", up, true);
    addEventListener("pointercancel", stop, true);
    addEventListener("keydown", key, true);
  }

  private goalCard(now: number): string {
    const goal = this.goalShown;
    if (!this.goals || !goal) return "";
    const words = this.words.goal;
    const worked = goal.worked + (goal.status === "active" ? Math.max(0, now - goal.since) : 0);
    const state = `${words.states[goal.status]}　${words.worked(worked)}`;
    const body = `<button class="sh-open sh-clamp" type="button" data-open="goal">${escape(goal.objective)}</button>`;
    return this.card("goal", this.words.pages.goal, state, body, { waits: goal.status === "blocked", barred: this.barred });
  }

  /** The plan read whole in its card: how far it is after its name, in
   *  clay while it asks the user. */
  private planCard(): string {
    if (!this.planned) return "";
    const words = this.words;
    const shape = this.planShown;
    let count = "";
    const here = shape?.stops.findIndex((stop) => stop.where === "here") ?? -1;
    const walked = shape?.stops.filter((stop) => stop.where === "walked").length ?? 0;
    if (shape && (here >= 0 || walked > 0)) count = words.planCount((here >= 0 ? here : walked - 1) + 1, shape.stops.length);
    else if (shape?.checks.length) count = words.planCount(shape.checks.filter((check) => check.done).length, shape.checks.length);
    const asks = shape?.asks.length ? words.collab.states.waiting : "";
    return this.card("plan", words.pages.plan, count, `<div class="sh-plan"></div>`, { waiting: asks });
  }

  /** The files the conversation changed: how many and by how many lines
   *  after the name; every one of them a line, the last changed first,
   *  scrolling in the card; or, turned by the mark at its head, their page:
   *  the list in full and the folders it works in. Both are in the card,
   *  one of them shown. */
  private filesCard(): string {
    const words = this.fileWords;
    if (!words || !this.changed.length) return "";
    const sum = tally(this.changed);
    const state = words.files(sum.files);
    const tail = figures(sum.added, sum.removed);
    const lines = [...this.changed]
      .sort((a, b) => (b.changes[0]?.at ?? 0) - (a.changes[0]?.at ?? 0))
      .map((file) => `<button class="sh-file" type="button" data-file="${escape(file.path)}" data-machine="${escape(file.machine)}" title="${escape(file.path)}">${mark(file.how, words.how[file.how])}<span class="sh-file-name">${escape(file.name)}</span>${figures(file.added, file.removed)}</button>`)
      .join("");
    const listed = `<div class="sh-files">${lines}</div>`;
    if (!this.pages.files) return this.card("files", words.side.name, state, listed, { tail });
    const view = (id: "lines" | "page", inner: string) => `<div class="sh-view${(id === "page") === this.browsing ? " shown" : ""}" data-view="${id}">${inner}</div>`;
    // Which of the two it shows is turned where the card stands.
    return this.card("files", words.side.name, state, `<div class="sh-views">${view("lines", listed)}${view("page", `<div class="sh-frame"></div>`)}</div>`, { tail, marks: this.browseMark(), sign: [state, tail, lines].join("\u0001") });
  }

  /** The colleagues, or the host's agents: how many work now after the
   *  name, and their page in the card, every one of them a row. */
  private teamCard(card: "colleagues" | "agents", nodes: CollabNode[]): string {
    if (nodes.length === 0) return "";
    const words = this.words;
    const working = nodes.filter((node) => going(node.state)).length;
    const waiting = nodes.some((node) => node.state === "waiting");
    const name = card === "colleagues" ? words.collab.groups.colleagues : words.collab.groups.agents;
    return this.card(card, name, working ? words.collab.atWork(working) : "", `<div class="sh-frame"></div>`, { waiting: waiting ? words.collab.states.waiting : "" });
  }
}
