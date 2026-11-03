// The side of the conversations shown (`world-gui.md`, 对话的侧面板): folded
// away until asked for, then a column of the desk beside the conversations,
// set apart from them by the margin ruled along the edge they share alone.
// It holds the parts of the conversation in focus, the first of them its
// overview, where it opens; any other, and a file read over the parts, has
// the way back to it over the page. Another conversation in focus, the side
// turns to its parts. Its "⋯", where it stands, is held in the desk's
// corner with where the conversation works and the side's switch, which
// stay as it folds away. The side opens and folds by the desk's `--sp-open`
// (`side.css`): folding, it stays drawn until that has gone back to
// nothing. Where there is no room beside the conversations, as on a phone,
// it lies over the whole desk, where the conversation works said in its
// own line.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { msOf, reducedMotion } from "../motion";
import { Sketch } from "../pen/sketch";
import type { SideWhere } from "./where";
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

/** One of a conversation's own parts, a page of the side; the first is its
 *  overview. */
export interface PanePart {
  id: string;
  name: string;
  page: SidePage;
}

/** A conversation's parts, as the side shows them while it is in focus. */
export interface PartSet {
  readonly parts: PanePart[];
  /** Where the conversation works. */
  readonly where: SideWhere;
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
  /** Its "⋯" and the menu of where it stands, held in the desk's corner. */
  readonly more: HTMLElement;
  private readonly sheet: HTMLElement;
  private readonly whereSlot: HTMLElement;
  private readonly back: HTMLElement;
  private readonly body: HTMLElement;
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
    this.element.innerHTML = `<div class="sp-sheet"><div class="sp-margin" aria-hidden="true"></div><header class="sp-head"><div class="sp-line"><div class="sp-where"></div><div class="sp-acts"><button class="icon-button sp-shut" type="button" aria-label="${escape(words.shut)}" title="${escape(words.shut)}">${glyph("close", 18)}</button></div></div></header><div class="sp-back" hidden></div><div class="sp-body"></div></div>`;
    // Where the side stands is the side's own to choose: its "⋯" is there
    // while the side is.
    this.more = document.createElement("div");
    this.more.className = "sp-dock";
    this.more.hidden = true;
    this.more.innerHTML = `<button class="icon-button sp-more" type="button" aria-haspopup="menu" aria-expanded="false" aria-label="${escape(words.more)}" title="${escape(words.more)}">${glyph("more", 18)}</button><div class="sp-menu" role="menu" hidden>${(["right", "bottom", "left"] as const)
      .map((dock) => `<button class="sp-menu-row" type="button" role="menuitemradio" aria-checked="false" data-dock="${dock}">${glyph(`dock-${dock}`, 16)}<span>${escape(words.docks[dock])}</span></button>`)
      .join("")}</div>`;
    this.sheet = this.element.querySelector(".sp-sheet")!;
    this.whereSlot = this.element.querySelector(".sp-where")!;
    this.back = this.element.querySelector(".sp-back")!;
    this.body = this.element.querySelector(".sp-body")!;
    this.margin = this.element.querySelector(".sp-margin")!;
    this.menu = this.more.querySelector(".sp-menu")!;
    const more = this.more.querySelector<HTMLButtonElement>(".sp-more")!;
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
    this.back.addEventListener("click", (event) => {
      const home = this.set?.parts[0]?.id;
      if (home && (event.target as HTMLElement).closest(".sp-home")) this.select(home);
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

  /** The part in view now: none while it is shut, or while a file is
   *  read over the parts. */
  get visible(): string | null {
    return this.open ? this.shown() : null;
  }

  /** Still drawn as it shuts. */
  get leaving(): boolean {
    return this.shutting !== 0;
  }

  /** Turns the side to another conversation's parts, or to none: what was
   *  read over the parts goes, the parts' pages are put in, its own line
   *  holds where that conversation works. */
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
    this.whereSlot.replaceChildren(...(set ? [set.where.line] : []));
    this.drawShown();
    if (this.open) this.page()?.shown?.();
  }

  /** The way back drawn again, as what the side shows changed. */
  restate(): void {
    this.drawBack();
  }

  /** Opens the side: at `part` when one is named, else where it was left. */
  show(part?: string | null): void {
    if (this.shutting) {
      window.clearTimeout(this.shutting);
      this.shutting = 0;
    }
    this.element.hidden = false;
    this.more.hidden = false;
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
    this.openMenu(false);
    this.more.hidden = true;
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
    this.more.querySelector(".sp-more")!.setAttribute("aria-expanded", String(open));
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
    this.drawBack();
  }

  /** Over the page: on the overview nothing; on any other page, and over
   *  the parts, the way back to the overview, named by it. */
  private drawBack(): void {
    const home = this.set?.parts[0];
    const away = !!home && (this.peeked !== null || this.shown() !== home.id);
    const html = away ? `<button class="text-button page-back sp-home" type="button">${glyph("back", 18)}<span>${escape(home.name)}</span></button>` : "";
    if (this.back.innerHTML !== html) this.back.innerHTML = html;
    this.back.hidden = !away;
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
    // Nothing is ruled between the side and the conversations: its cards
    // stand on the page. The line comes up only while it is held or pointed
    // at, to be dragged.
    this.margin.innerHTML = `<div class="sp-margin-ink">${stroke("current")}</div>`;
  }
}
