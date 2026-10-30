// The side of the conversations shown (`world-gui.md`, 对话的侧面板): shut
// until asked for, then a column of the desk beside the conversations, set
// apart from them by the margin ruled along the edge they share alone. It
// holds the parts of the conversation in focus: its first line is their
// marks, the goal, the plan and who works with it, how far the plan is and
// how many work with it in small figures after them, a mark in clay while
// something there waits on the user, the one shown on the clay wash; then the
// side's "⋯". Another conversation in focus, the side turns to its parts.
// One of the host's agents, opened from who works with it, is read in the
// side's body until a part's mark is pressed again. What the conversation is
// pointed at stands at the foot under its parts. The side opens and shuts by
// the desk's `--sp-open` (`side.css`): shutting, it stays drawn until that
// has gone back to nothing. Where there is no room beside the conversations,
// as on a phone, it lies over the whole desk.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { msOf, reducedMotion } from "../motion";
import { Sketch } from "../pen/sketch";
import type { SideWords } from "./words";
import "./side.css";

/** A page drawn in the side. */
export interface SidePage {
  readonly element: HTMLElement;
  /** It came into view: it may draw what needs its size. */
  shown?(): void;
}

/** Where the side stands beside the conversations. */
export type Dock = "right" | "bottom" | "left";

/** One of a conversation's own parts, a mark in the side's first line. */
export interface PanePart {
  id: string;
  name: string;
  glyph: "goal" | "rows" | "tree";
  page: SidePage;
}

/** How a part stands, drawn on its mark. */
export interface PartState {
  /** A few figures after the mark: how far the plan is, how many work. */
  count?: string;
  /** Something there waits on the user: the mark is in clay. */
  waiting?: boolean;
  /** What pointing at it says after its name. */
  said?: string;
}

/** A conversation's parts, as the side shows them while it is in focus. */
export interface PartSet {
  readonly parts: PanePart[];
  state(id: string): PartState | undefined;
  /** What the conversation is pointed at, for the foot. */
  readonly env: HTMLElement | null;
  /** The part this conversation last showed. */
  part: string | null;
}

/** Something read in the side's body over the parts: one of the host's
 *  agents. */
export interface Peek {
  readonly element: HTMLElement;
  leave(): void;
}

export class SidePane {
  readonly element: HTMLElement;
  private readonly sheet: HTMLElement;
  private readonly partsRow: HTMLElement;
  private readonly body: HTMLElement;
  private readonly foot: HTMLElement;
  private readonly margin: HTMLElement;
  private readonly menu: HTMLElement;
  private set: PartSet | null = null;
  private peeked: Peek | null = null;
  private height = 0;
  private dock: Dock = "right";
  /** Shutting: still drawn while it goes. */
  private shutting = 0;

  constructor(
    words: SideWords,
    private readonly seams: {
      /** The side opened or shut, or turned to another part. */
      changed: () => void;
      /** The user chose where the side stands. */
      dock: (dock: Dock) => void;
    },
  ) {
    this.element = document.createElement("aside");
    this.element.className = "sp-panel";
    this.element.hidden = true;
    this.element.setAttribute("aria-label", words.panel);
    this.element.innerHTML = `<div class="sp-sheet"><div class="sp-margin" aria-hidden="true"></div><header class="sp-head"><div class="sp-line"><div class="sp-parts" role="tablist" aria-label="${escape(words.panel)}"></div><div class="sp-acts"><button class="icon-button sp-more" type="button" aria-haspopup="menu" aria-expanded="false" aria-label="${escape(words.more)}" title="${escape(words.more)}">${glyph("more", 18)}</button><button class="icon-button sp-shut" type="button" aria-label="${escape(words.shut)}" title="${escape(words.shut)}">${glyph("close", 18)}</button><div class="sp-menu" role="menu" hidden>${(["right", "bottom", "left"] as const)
      .map((dock) => `<button class="sp-menu-row" type="button" role="menuitemradio" aria-checked="false" data-dock="${dock}">${glyph(`dock-${dock}`, 16)}<span>${escape(words.docks[dock])}</span></button>`)
      .join("")}</div></div></div></header><div class="sp-body"></div><footer class="sp-foot"></footer></div>`;
    this.sheet = this.element.querySelector(".sp-sheet")!;
    this.partsRow = this.element.querySelector(".sp-parts")!;
    this.body = this.element.querySelector(".sp-body")!;
    this.foot = this.element.querySelector(".sp-foot")!;
    this.margin = this.element.querySelector(".sp-margin")!;
    this.menu = this.element.querySelector(".sp-menu")!;
    const more = this.element.querySelector<HTMLButtonElement>(".sp-more")!;
    more.addEventListener("click", () => this.openMenu(this.menu.hidden));
    this.menu.addEventListener("click", (event) => {
      const dock = (event.target as HTMLElement).closest<HTMLElement>(".sp-menu-row")?.dataset.dock as Dock | undefined;
      if (!dock) return;
      this.openMenu(false);
      this.seams.dock(dock);
    });
    document.addEventListener("pointerdown", (event) => {
      const target = event.target as HTMLElement;
      if (!this.menu.hidden && !this.menu.contains(target) && !more.contains(target)) this.openMenu(false);
    });
    this.element.querySelector(".sp-shut")!.addEventListener("click", () => this.close());
    this.partsRow.addEventListener("click", (event) => {
      const id = (event.target as HTMLElement).closest<HTMLElement>("[data-part]")?.dataset.part;
      if (id) this.select(id);
    });
    this.partsRow.addEventListener("keydown", (event) => {
      const at = (event.target as HTMLElement).closest<HTMLElement>("[data-part]")?.dataset.part;
      const step = event.key === "ArrowRight" ? 1 : event.key === "ArrowLeft" ? -1 : 0;
      const all = this.set?.parts.map((part) => part.id) ?? [];
      if (!at || !step || !all.length) return;
      event.preventDefault();
      const next = all[(all.indexOf(at) + step + all.length) % all.length]!;
      this.select(next);
      this.partsRow.querySelector<HTMLElement>(`[data-part="${CSS.escape(next)}"]`)?.focus();
    });
    this.element.addEventListener("keydown", (event) => {
      if (event.key === "Escape" && this.element.classList.contains("sp-over")) this.close();
    });
    new ResizeObserver(() => this.rule()).observe(this.sheet);
  }

  /** Open, or opening: not while it shuts. */
  get open(): boolean {
    return !this.element.hidden && !this.shutting;
  }

  /** Still drawn as it shuts. */
  get leaving(): boolean {
    return this.shutting !== 0;
  }

  /** Turns the side to another conversation's parts, or to none: what was
   *  read over the parts goes, the parts' pages are put in, the foot holds
   *  what that conversation is pointed at. */
  use(set: PartSet | null): void {
    if (set === this.set) return;
    this.unpeek();
    for (const part of this.set?.parts ?? []) part.page.element.remove();
    this.set = set;
    for (const part of set?.parts ?? []) {
      part.page.element.classList.add("sp-page");
      part.page.element.setAttribute("role", "tabpanel");
      this.body.append(part.page.element);
    }
    this.foot.replaceChildren(...(set?.env ? [set.env] : []));
    this.drawShown();
    if (this.open) this.page()?.shown?.();
  }

  /** The marks drawn again, as the parts' states changed. */
  restate(): void {
    this.drawParts();
  }

  /** Opens the side: at `part` when one is named, else where it was left. */
  show(part?: string | null): void {
    if (this.shutting) {
      window.clearTimeout(this.shutting);
      this.shutting = 0;
    }
    this.element.hidden = false;
    if (part && this.set?.parts.some((one) => one.id === part)) this.select(part);
    else {
      this.drawShown();
      this.page()?.shown?.();
      this.seams.changed();
    }
  }

  /** Shuts it: it goes as `--sp-open` goes back, then is no longer drawn. */
  close(): void {
    if (!this.open) return;
    const ms = reducedMotion() ? 0 : msOf(getComputedStyle(this.element).getPropertyValue("--glide"));
    this.shutting = window.setTimeout(() => {
      this.shutting = 0;
      this.element.hidden = true;
      this.unpeek();
      this.seams.changed();
    }, ms);
    this.seams.changed();
  }

  toggle(first?: string | null): void {
    if (this.open) this.close();
    else this.show(this.set?.part ?? first);
  }

  /** Shows a part, what was read over the parts going. */
  select(id: string): void {
    if (!this.set?.parts.some((part) => part.id === id)) return;
    this.unpeek();
    this.set.part = id;
    this.drawShown();
    if (this.open) this.page()?.shown?.();
    this.seams.changed();
  }

  /** Reads `peek` in the body over the parts, until a part is chosen. */
  peek(peek: Peek): void {
    this.unpeek();
    this.peeked = peek;
    peek.element.classList.add("sp-page", "sp-peek");
    this.body.append(peek.element);
    this.drawShown();
  }

  /** Where there is no room beside the conversations, as on a phone, the
   *  side lies over the whole desk. */
  over(over: boolean): void {
    this.element.classList.toggle("sp-over", over);
  }

  /** Where the side stands: its margin is ruled along the edge it shares
   *  with the conversations, and its menu marks the place. */
  setDock(dock: Dock): void {
    this.dock = dock;
    this.height = 0;
    this.rule();
    for (const row of this.menu.querySelectorAll<HTMLElement>("[data-dock]")) row.setAttribute("aria-checked", String(row.dataset.dock === dock));
  }

  /** The places the desk has room for the side at now; the others are not
   *  offered. */
  fitting(docks: readonly Dock[]): void {
    for (const row of this.menu.querySelectorAll<HTMLButtonElement>("[data-dock]")) {
      const disabled = !docks.includes(row.dataset.dock as Dock);
      if (row.disabled !== disabled) row.disabled = disabled;
    }
  }

  private unpeek(): void {
    const peeked = this.peeked;
    if (!peeked) return;
    this.peeked = null;
    peeked.element.remove();
    peeked.leave();
  }

  private openMenu(open: boolean): void {
    this.menu.hidden = !open;
    this.element.querySelector(".sp-more")!.setAttribute("aria-expanded", String(open));
  }

  /** The part shown, unless something is read over the parts. */
  private shown(): string | null {
    if (this.peeked || !this.set) return null;
    return this.set.part ?? this.set.parts[0]?.id ?? null;
  }

  private page(): { shown?(): void } | null {
    const shown = this.shown();
    return this.set?.parts.find((part) => part.id === shown)?.page ?? null;
  }

  private drawShown(): void {
    const shown = this.shown();
    for (const part of this.set?.parts ?? []) part.page.element.hidden = part.id !== shown;
    // What the conversation is pointed at is said under its parts.
    this.foot.hidden = !shown || this.foot.childElementCount === 0;
    this.drawParts();
  }

  /** The first line's marks: each part's, with its figures after it, in
   *  clay while something there waits on the user; the one shown on the
   *  clay wash. */
  private drawParts(): void {
    const shown = this.shown();
    const set = this.set;
    this.partsRow.innerHTML = (set?.parts ?? [])
      .map((part) => {
        const chosen = part.id === shown;
        const state = set!.state(part.id) ?? {};
        const said = [part.name, state.said].filter(Boolean).join("\n");
        const count = state.count ? `<span class="sp-part-count">${escape(state.count)}</span>` : "";
        return `<button class="sp-part${chosen ? " chosen" : ""}${state.waiting ? " waiting" : ""}" type="button" role="tab" data-part="${escape(part.id)}" aria-selected="${chosen}" tabindex="${chosen || (!shown && part === set!.parts[0]) ? 0 : -1}" aria-label="${escape(said)}" title="${escape(said)}">${glyph(part.glyph, 18)}${count}</button>`;
      })
      .join("");
  }

  /** The margin is drawn again when the sheet's length along it changes,
   *  with the same stroke in ink over it for while it is held: down the
   *  sheet's edge beside the conversations from end to end, or across its
   *  top at the foot. The stroke runs past both ends, so the line meets the
   *  window's edges with no lift. */
  private rule(): void {
    const across = this.dock === "bottom";
    const length = across ? this.sheet.clientWidth : this.sheet.clientHeight;
    if (length === 0 || length === this.height) return;
    this.height = length;
    const stroke = (ink: "graphite" | "current") =>
      across
        ? new Sketch("side-margin-across").rule([-16, 2], [length + 16, 2], 1, ink).svg(length, 4, "sp-margin-svg")
        : new Sketch("side-margin").rule([2, -16], [2, length + 16], 1, ink).svg(4, length, "sp-margin-svg");
    this.margin.innerHTML = `${stroke("graphite")}<div class="sp-margin-ink">${stroke("current")}</div>`;
  }
}
