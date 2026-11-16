// The side of the conversations shown (`world-gui.md`, 对话的侧面板): folded
// away until asked for, then a column of the desk beside the conversations,
// a sheet of its own a shade under the page, its edge ruled by hand. It
// holds the parts of the conversation in focus, the first of them its
// overview, where it opens; any other part, and each file read there, opens
// on a tab of its own after the overview's, as many as were opened, each
// shut by its cross. Another conversation in focus, the side turns to its
// parts and its tabs. Its "⋯", where it stands, is held in the desk's
// corner with where the conversation works and the side's switch, which
// stay as it folds away. The desk's open class determines the side's size
// (`side.css`): folding, it stays drawn until the optional size transition
// has had time to finish. Where there is no room beside it, as on a phone,
// it lies over the whole desk, where the conversation works said in its
// own line.

import { cardNames, cardPressed, cardRows, onCardsChosen } from "./shown";
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

/** A page open in the side on a tab of its own, after the overview's: one
 *  of the conversation's parts, or a file read there. */
export interface Tab {
  id: string;
  name: string;
  /** What tells it from another of the same name: a file's folder. */
  hint?: string | undefined;
  /** The whole of what it is, for the pointer resting on its name. */
  title?: string | undefined;
  /** The file it reads, where it reads one. */
  file?: { machine: string; path: string } | undefined;
  page: SidePage;
  /** It was shut: what it held is let go. A part's page has none, and is
   *  kept for when the part is opened again. */
  leave?(): void;
}

/** A conversation's parts, as the side shows them while it is in focus. */
export interface PartSet {
  readonly parts: PanePart[];
  /** The pages open beside the overview, each on its tab, in the order
   *  they were opened. */
  readonly tabs: Tab[];
  /** Where the conversation works. */
  readonly where: SideWhere;
  /** The tab this conversation last showed: the overview's part, or one of
   *  `tabs`. */
  part: string | null;
  /** Its history's part, and what the mark that opens it is named. */
  readonly history: { part: string; label: string } | null;
}

export class SidePane {
  readonly element: HTMLElement;
  /** Its "⋯" and the menu of where it stands, held in the desk's corner. */
  readonly more: HTMLElement;
  private readonly sheet: HTMLElement;
  private readonly whereSlot: HTMLElement;
  /** The line of tabs, and the tabs in it. */
  private readonly strip: HTMLElement;
  private readonly run: HTMLElement;
  private drawnTabs = "";
  private readonly body: HTMLElement;
  private readonly margin: HTMLElement;
  private readonly menu: HTMLElement;
  /** Opens the history, where the side lies over the desk and the corner's
   *  mark is not drawn. */
  private readonly historyMark: HTMLButtonElement;
  private set: PartSet | null = null;
  private height = 0;
  private dock: Dock = "right";
  /** Shutting: still drawn while it goes. */
  private shutting = 0;

  constructor(
    private readonly words: SideWords,
    private readonly seams: {
      /** The side opened or shut, or turned to another part. */
      changed: () => void;
      /** The user chose where the side stands. */
      dock: (dock: Dock) => void;
      /** The user took the files read on tabs out into a window of their
       *  own; none where that cannot be. */
      out?: (() => void) | undefined;
      /** The user asked what stands alone in this window back into the
       *  desk. */
      back?: (() => void) | undefined;
    },
  ) {
    this.element = document.createElement("aside");
    this.element.className = "sp-panel";
    this.element.hidden = true;
    this.element.setAttribute("aria-label", words.panel);
    this.element.innerHTML = `<div class="sp-sheet"><div class="sp-margin" aria-hidden="true"></div><header class="sp-head"><div class="sp-line"><div class="sp-where"></div><div class="sp-acts"><button class="icon-button sp-history-mark" type="button" hidden>${glyph("journal", 18)}</button>${seams.back ? `<button class="icon-button sp-put-back" type="button" aria-label="${escape(words.board.back)}" title="${escape(words.board.back)}">${glyph("dock-right", 18)}</button>` : `<button class="icon-button sp-shut" type="button" aria-label="${escape(words.shut)}" title="${escape(words.shut)}">${glyph("close", 18)}</button>`}</div></div></header><div class="sp-tabs" hidden><div class="sp-tabs-run" role="tablist"></div>${seams.out ? `<button class="icon-button sp-out" type="button" aria-label="${escape(words.board.out)}" title="${escape(words.board.out)}">${glyph("dock-out", 18)}</button>` : ""}</div><div class="sp-body"></div></div>`;
    // Where the side stands is the side's own to choose: its "⋯" is there
    // while the side is.
    this.more = document.createElement("div");
    // Away while the side is shut, not hidden: it leaves the corner's line
    // and comes back into it at the side's own pace (`side.css`).
    this.more.className = "sp-dock away";
    this.more.inert = true;
    this.more.innerHTML = `<span class="sp-more-room"><button class="icon-button sp-more" type="button" aria-haspopup="menu" aria-expanded="false" aria-label="${escape(words.more)}" title="${escape(words.more)}">${glyph("more", 18)}</button></span><div class="sp-menu" role="menu" hidden><div class="sp-menu-cards"></div><div class="sp-menu-rule" aria-hidden="true"></div>${(["right", "bottom", "left"] as const)
      .map((dock) => `<button class="sp-menu-row" type="button" role="menuitemradio" aria-checked="false" data-dock="${dock}">${glyph(`dock-${dock}`, 16)}<span>${escape(words.docks[dock])}</span></button>`)
      .join("")}</div>`;
    this.sheet = this.element.querySelector(".sp-sheet")!;
    this.whereSlot = this.element.querySelector(".sp-where")!;
    this.strip = this.element.querySelector(".sp-tabs")!;
    this.run = this.strip.querySelector(".sp-tabs-run")!;
    this.body = this.element.querySelector(".sp-body")!;
    this.margin = this.element.querySelector(".sp-margin")!;
    this.menu = this.more.querySelector(".sp-menu")!;
    const more = this.more.querySelector<HTMLButtonElement>(".sp-more")!;
    more.addEventListener("click", () => this.openMenu(this.menu.hidden));
    // What the overview shows, over where the side stands: the same rows
    // the overview's own menu has, drawn again as they are chosen.
    const cards = this.menu.querySelector<HTMLElement>(".sp-menu-cards")!;
    const names = cardNames(words);
    const drawCards = () => {
      cards.innerHTML = cardRows(names);
    };
    drawCards();
    onCardsChosen(drawCards);
    this.menu.addEventListener("click", (event) => {
      if (cardPressed(event.target as HTMLElement)) return;
      const dock = (event.target as HTMLElement).closest<HTMLElement>(".sp-menu-row")?.dataset.dock as Dock | undefined;
      if (!dock) return;
      this.openMenu(false);
      this.seams.dock(dock);
    });
    document.addEventListener("pointerdown", (event) => {
      const target = event.target as HTMLElement;
      if (!this.menu.hidden && !this.menu.contains(target) && !more.contains(target)) this.openMenu(false);
    });
    this.element.querySelector(".sp-shut")?.addEventListener("click", () => this.close());
    this.element.querySelector(".sp-put-back")?.addEventListener("click", () => this.seams.back?.());
    this.strip.querySelector(".sp-out")?.addEventListener("click", () => this.seams.out?.());
    this.historyMark = this.element.querySelector(".sp-history-mark")!;
    this.historyMark.addEventListener("click", () => this.turnToHistory());
    const tabAt = (event: Event) => (event.target as HTMLElement).closest<HTMLElement>("[data-tab]")?.dataset.tab;
    this.run.addEventListener("click", (event) => {
      const id = tabAt(event);
      if (!id) return;
      if ((event.target as HTMLElement).closest(".sp-tab-shut")) this.shut(id);
      else this.select(id);
    });
    // The middle button shuts a tab, as a browser's does.
    this.run.addEventListener("auxclick", (event) => {
      const id = event.button === 1 ? tabAt(event) : undefined;
      if (!id) return;
      event.preventDefault();
      this.shut(id);
    });
    this.run.addEventListener("keydown", (event) => {
      const ids = this.ids();
      const at = ids.indexOf(tabAt(event) ?? "");
      if (at < 0) return;
      const to = event.key === "ArrowRight" ? ids[at + 1] : event.key === "ArrowLeft" ? ids[at - 1] : event.key === "Home" ? ids[0] : event.key === "End" ? ids[ids.length - 1] : undefined;
      if (to) {
        event.preventDefault();
        this.select(to);
        this.run.querySelector<HTMLElement>(".sp-tab.chosen .sp-tab-name")?.focus();
      } else if (event.key === "Delete") {
        event.preventDefault();
        this.shut(ids[at]!);
        this.run.querySelector<HTMLElement>(".sp-tab.chosen .sp-tab-name")?.focus();
      }
    });
    // A wheel turned over the tabs moves along them.
    this.run.addEventListener(
      "wheel",
      (event) => {
        if (this.run.scrollWidth <= this.run.clientWidth || Math.abs(event.deltaY) <= Math.abs(event.deltaX)) return;
        event.preventDefault();
        this.run.scrollLeft += event.deltaY;
      },
      { passive: false },
    );
    this.element.addEventListener("keydown", (event) => {
      if (event.key === "Escape" && this.element.classList.contains("sp-over") && !this.seams.back) this.close();
    });
    new ResizeObserver(() => this.rule()).observe(this.sheet);
  }

  /** Open, or opening: not while it shuts. */
  get open(): boolean {
    return !this.element.hidden && !this.shutting;
  }

  /** The tab in view now, a part's or a file's: none while it is shut. */
  get visible(): string | null {
    return this.open ? this.shown() : null;
  }

  /** Still drawn as it shuts. */
  get leaving(): boolean {
    return this.shutting !== 0;
  }

  /** Turns the side to another conversation's parts and tabs, or to none:
   *  the pages of the one it showed are taken out, kept as they are for
   *  when it is in focus again; the other's are put in, its own line holds
   *  where that conversation works. */
  use(set: PartSet | null): void {
    if (set === this.set) return;
    for (const page of this.pages()) page.element.remove();
    this.set = set;
    for (const page of this.pages()) this.put(page);
    this.whereSlot.replaceChildren(...(set ? [set.where.line] : []));
    const label = set?.history?.label ?? "";
    this.historyMark.hidden = !set?.history;
    this.historyMark.title = label;
    this.historyMark.setAttribute("aria-label", label);
    this.drawShown();
    if (this.open) this.page()?.shown?.();
  }

  /** The tabs drawn again, as what the side shows changed. */
  restate(): void {
    this.drawTabs();
  }

  /** Opens the side: at `part` when one is named, else where it was left. */
  show(part?: string | null): void {
    if (this.shutting) {
      window.clearTimeout(this.shutting);
      this.shutting = 0;
    }
    this.element.hidden = false;
    this.more.classList.remove("away");
    this.more.inert = false;
    if (part && this.has(part)) this.select(part);
    else {
      this.drawShown();
      this.page()?.shown?.();
      this.seams.changed();
    }
  }

  /** Shuts it immediately; it stays drawn briefly for the size transition. */
  close(): void {
    if (!this.open) return;
    this.openMenu(false);
    this.more.classList.add("away");
    this.more.inert = true;
    const ms = reducedMotion() ? 0 : msOf(getComputedStyle(this.element).getPropertyValue("--glide"));
    this.shutting = window.setTimeout(() => {
      this.shutting = 0;
      this.element.hidden = true;
      this.seams.changed();
    }, ms);
    this.seams.changed();
  }

  toggle(first?: string | null): void {
    if (this.open) this.close();
    else this.show(this.set?.part ?? first);
  }

  /** Opens the side at the history, or, showing it, back at the overview. */
  turnToHistory(): void {
    const history = this.set?.history;
    if (!history) return;
    if (this.visible === history.part) this.select(this.set!.parts[0]!.id);
    else this.show(history.part);
  }

  /** Turns to a tab. A part other than the overview that has none yet
   *  opens on one, after those open. */
  select(id: string): void {
    const set = this.set;
    if (!set || !this.has(id)) return;
    const part = set.parts.find((one) => one.id === id);
    if (part && part !== set.parts[0] && !set.tabs.some((tab) => tab.id === id)) set.tabs.push({ id, name: part.name, page: part.page });
    set.part = id;
    this.drawShown();
    if (this.open) this.page()?.shown?.();
    this.seams.changed();
  }

  /** Opens `tab` after those open and turns to it; one already open is
   *  only turned to. */
  add(tab: Tab): void {
    const set = this.set;
    if (!set) return;
    if (!set.tabs.some((one) => one.id === tab.id)) {
      set.tabs.push(tab);
      this.put(tab.page);
    }
    this.select(tab.id);
  }

  /** Shuts a tab: what it held is let go, and the one that takes its place
   *  is turned to where it was the one in view. */
  shut(id: string): void {
    const set = this.set;
    const at = set?.tabs.findIndex((tab) => tab.id === id) ?? -1;
    if (!set || at < 0) return;
    const [tab] = set.tabs.splice(at, 1);
    if (tab!.leave) {
      tab!.page.element.remove();
      tab!.leave();
    }
    if (set.part === id) this.select(set.tabs[at]?.id ?? set.tabs[at - 1]?.id ?? set.parts[0]!.id);
    else this.drawShown();
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

  private openMenu(open: boolean): void {
    this.menu.hidden = !open;
    this.more.querySelector(".sp-more")!.setAttribute("aria-expanded", String(open));
  }

  /** Whether `id` is a part of the conversation shown, or a tab open. */
  private has(id: string): boolean {
    return !!this.set && (this.set.parts.some((part) => part.id === id) || this.set.tabs.some((tab) => tab.id === id));
  }

  /** The tabs in the order they stand: the overview's, then those open. */
  private ids(): string[] {
    const home = this.set?.parts[0];
    return home ? [home.id, ...this.set!.tabs.map((tab) => tab.id)] : [];
  }

  /** Every page of the conversation shown: its parts', and its tabs'. */
  private pages(): SidePage[] {
    const set = this.set;
    return set ? [...new Set([...set.parts.map((part) => part.page), ...set.tabs.map((tab) => tab.page)])] : [];
  }

  private put(page: SidePage): void {
    page.element.classList.add("sp-page");
    page.element.setAttribute("role", "tabpanel");
    if (page.element.parentElement !== this.body) this.body.append(page.element);
  }

  /** The tab in view: the one last shown while it is still there, else the
   *  overview's. */
  private shown(): string | null {
    const set = this.set;
    if (!set) return null;
    return set.part && this.ids().includes(set.part) ? set.part : (set.parts[0]?.id ?? null);
  }

  private page(): SidePage | null {
    const shown = this.shown();
    return this.set?.tabs.find((tab) => tab.id === shown)?.page ?? this.set?.parts.find((part) => part.id === shown)?.page ?? null;
  }

  private drawShown(): void {
    const shown = this.page();
    for (const page of this.pages()) page.element.hidden = page !== shown;
    const id = this.shown();
    this.historyMark.setAttribute("aria-pressed", String(!!id && id === this.set?.history?.part));
    this.drawTabs();
  }

  /** The line of tabs: none with the overview alone; else the overview's,
   *  then each open, the one in view marked and brought into the line's
   *  view. Two of one name each say what tells them apart. */
  private drawTabs(): void {
    const set = this.set;
    const home = set?.parts[0];
    const tabs = set?.tabs ?? [];
    const shown = this.shown();
    const named = new Map<string, number>();
    for (const tab of tabs) named.set(tab.name, (named.get(tab.name) ?? 0) + 1);
    const one = (id: string, name: string, hint: string, title: string, shut: boolean) =>
      `<span class="sp-tab${id === shown ? " chosen" : ""}${shut ? "" : " fixed"}" data-tab="${escape(id)}"${title ? ` title="${escape(title)}"` : ""}><button class="sp-tab-name" type="button" role="tab" aria-selected="${id === shown}" tabindex="${id === shown ? 0 : -1}"><span>${escape(name)}</span>${hint ? `<span class="sp-tab-hint">${escape(hint)}</span>` : ""}</button>${shut ? `<button class="sp-tab-shut" type="button" tabindex="-1" aria-label="${escape(`${this.words.close} ${name}`)}">${glyph("close", 14)}</button>` : ""}</span>`;
    const html = home && tabs.length ? one(home.id, home.name, "", "", false) + tabs.map((tab) => one(tab.id, tab.name, named.get(tab.name)! > 1 ? (tab.hint ?? "") : "", tab.title ?? "", true)).join("") : "";
    if (html !== this.drawnTabs) {
      this.drawnTabs = html;
      this.run.innerHTML = html;
    }
    this.strip.hidden = !html;
    const chosen = this.run.querySelector<HTMLElement>(".sp-tab.chosen");
    if (!chosen) return;
    const left = chosen.offsetLeft;
    const right = left + chosen.offsetWidth;
    if (left < this.run.scrollLeft) this.run.scrollLeft = left;
    else if (right > this.run.scrollLeft + this.run.clientWidth) this.run.scrollLeft = right - this.run.clientWidth;
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
